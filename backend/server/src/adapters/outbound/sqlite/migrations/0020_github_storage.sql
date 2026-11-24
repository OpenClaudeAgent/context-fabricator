-- Logical payload only: byte lengths of retained records, excluding SQLite indexes,
-- free pages, WAL, filesystem allocation and unrelated collections.
CREATE TABLE github_storage_budget (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    revision INTEGER NOT NULL CHECK (revision >= 0),
    limit_bytes INTEGER NOT NULL CHECK (limit_bytes > 0)
) STRICT;
INSERT INTO github_storage_budget VALUES (1, 0, 10000000000);

-- Complete 0016 for development databases created before its blob-retention
-- portion was finalized. Existing evidence without blobs remains unquotable.
CREATE TABLE IF NOT EXISTS github_history_blobs (
    blob TEXT PRIMARY KEY,
    bytes BLOB NOT NULL
) STRICT;
CREATE TABLE IF NOT EXISTS github_history_evidence_blobs (
    repository_id INTEGER NOT NULL, branch TEXT NOT NULL, child TEXT NOT NULL,
    parent TEXT NOT NULL, projection_version INTEGER NOT NULL,
    blob TEXT NOT NULL REFERENCES github_history_blobs(blob),
    PRIMARY KEY (repository_id, branch, child, parent, projection_version, blob),
    FOREIGN KEY (repository_id, branch, child, parent, projection_version)
      REFERENCES github_history_evidence (repository_id, branch, child, parent, projection_version)
      ON DELETE CASCADE
) STRICT;
CREATE TRIGGER IF NOT EXISTS github_history_orphan_blob AFTER DELETE ON github_history_evidence_blobs
BEGIN
    DELETE FROM github_history_blobs WHERE blob = OLD.blob
      AND NOT EXISTS (SELECT 1 FROM github_history_evidence_blobs WHERE blob = OLD.blob);
END;

CREATE VIEW github_storage_usage AS
SELECT 'sources' AS family,
 COALESCE((SELECT sum(length(CAST(content AS BLOB))) FROM source_blobs WHERE collection IN (SELECT collection FROM github_code_scopes UNION SELECT collection FROM github_graph_scopes)),0)
 + COALESCE((SELECT sum(length(CAST(path AS BLOB))+length(CAST(source AS BLOB))+length(CAST(blob AS BLOB))+length(CAST(file_url AS BLOB))) FROM source_snapshot_files WHERE collection IN (SELECT collection FROM github_code_scopes UNION SELECT collection FROM github_graph_scopes)),0)
 + COALESCE((SELECT sum(length(CAST(revision AS BLOB))+length(CAST(tracked_ref AS BLOB))+length(CAST(coalesce(cursor,'') AS BLOB))) FROM source_snapshots WHERE collection IN (SELECT collection FROM github_code_scopes UNION SELECT collection FROM github_graph_scopes)),0)
 + COALESCE((SELECT sum(length(CAST(title AS BLOB))+length(CAST(content AS BLOB))+length(CAST(coalesce(source_location,'') AS BLOB))) FROM documents WHERE collection IN (SELECT collection FROM github_code_scopes UNION SELECT collection FROM github_graph_scopes)),0)
 + COALESCE((SELECT sum(length(v.embedding)+length(CAST(v.profile AS BLOB))) FROM document_vectors v JOIN documents d ON d.id=v.document_id WHERE d.collection IN (SELECT collection FROM github_code_scopes UNION SELECT collection FROM github_graph_scopes)),0) AS bytes
UNION ALL SELECT 'history',
 COALESCE((SELECT sum(length(CAST(snapshot AS BLOB))) FROM github_history_lineage),0)
 + COALESCE((SELECT sum(length(CAST(document AS BLOB))) FROM github_history_evidence),0)
 + COALESCE((SELECT sum(length(bytes)) FROM github_history_blobs),0)
UNION ALL SELECT 'actions', COALESCE((SELECT sum(length(CAST(snapshot AS BLOB))) FROM github_actions_snapshots),0)
UNION ALL SELECT 'graph',
 COALESCE((SELECT sum(length(CAST(roots AS BLOB))+length(CAST(coverage AS BLOB))+length(CAST(profile AS BLOB))+length(CAST(revision AS BLOB))) FROM github_graph_publications),0)
 + COALESCE((SELECT sum(length(CAST(record AS BLOB))+length(CAST(root AS BLOB))+length(CAST(name AS BLOB))+length(CAST(coalesce(qualified_name,'') AS BLOB))) FROM github_graph_occurrences),0);
-- Older development databases may have created these while 0016 was in flight.
DROP TRIGGER IF EXISTS github_history_evidence_removed;
DROP TRIGGER IF EXISTS github_history_evidence_scope_changed;
CREATE TABLE github_data_paused (
    repository_id INTEGER PRIMARY KEY CHECK (repository_id > 0)
) STRICT;

DROP VIEW github_current_eligible;
CREATE VIEW github_current_eligible AS
SELECT r.id AS repository_id, r.full_name, r.branch, r.interval_minutes,
       s.collection, c.generation, a.credential_ref
FROM github_repositories r
JOIN (
    SELECT collection, repository_id, branch, 1 AS general FROM github_code_scopes
    UNION ALL SELECT collection, repository_id, branch, 0 FROM github_graph_scopes
) s ON (s.repository_id, s.branch) = (r.id, r.branch)
JOIN collections c ON c.name = s.collection CROSS JOIN github_connection a
WHERE r.verified AND (r.current_code OR r.code_graph) AND s.general = r.current_code
  AND r.contents_access = 1 AND NOT c.access_lost AND NOT a.reconnect AND a.credential_ref IS NOT NULL
  AND NOT EXISTS (SELECT 1 FROM github_data_paused p WHERE p.repository_id = r.id);

-- The ledger is updated in the writer transaction. The view above remains an
-- independent full recount used for diagnostics and migration backfill.
CREATE TABLE github_storage_ledger (family TEXT PRIMARY KEY, bytes INTEGER NOT NULL CHECK (bytes >= 0)) STRICT;
INSERT INTO github_storage_ledger SELECT family, bytes FROM github_storage_usage;
-- Delete the child while its document owner is still visible to ledger triggers.
CREATE TRIGGER github_storage_document_vectors_before_delete BEFORE DELETE ON documents
BEGIN DELETE FROM document_vectors WHERE document_id = OLD.id; END;

CREATE TRIGGER github_budget_source_blobs_insert AFTER INSERT ON source_blobs
BEGIN UPDATE github_storage_ledger SET bytes = bytes + (CASE WHEN NEW.collection IN (SELECT collection FROM github_code_scopes UNION SELECT collection FROM github_graph_scopes) THEN length(CAST(NEW.content AS BLOB)) ELSE 0 END) WHERE family = 'sources'; END;

CREATE TRIGGER github_budget_source_blobs_delete AFTER DELETE ON source_blobs
BEGIN UPDATE github_storage_ledger SET bytes = bytes + (-(CASE WHEN OLD.collection IN (SELECT collection FROM github_code_scopes UNION SELECT collection FROM github_graph_scopes) THEN length(CAST(OLD.content AS BLOB)) ELSE 0 END)) WHERE family = 'sources'; END;

CREATE TRIGGER github_budget_source_blobs_update AFTER UPDATE ON source_blobs
BEGIN UPDATE github_storage_ledger SET bytes = bytes + ((CASE WHEN NEW.collection IN (SELECT collection FROM github_code_scopes UNION SELECT collection FROM github_graph_scopes) THEN length(CAST(NEW.content AS BLOB)) ELSE 0 END)-(CASE WHEN OLD.collection IN (SELECT collection FROM github_code_scopes UNION SELECT collection FROM github_graph_scopes) THEN length(CAST(OLD.content AS BLOB)) ELSE 0 END)) WHERE family = 'sources'; END;

CREATE TRIGGER github_budget_source_snapshot_files_insert AFTER INSERT ON source_snapshot_files
BEGIN UPDATE github_storage_ledger SET bytes = bytes + (CASE WHEN NEW.collection IN (SELECT collection FROM github_code_scopes UNION SELECT collection FROM github_graph_scopes) THEN length(CAST(NEW.path AS BLOB))+length(CAST(NEW.source AS BLOB))+length(CAST(NEW.blob AS BLOB))+length(CAST(NEW.file_url AS BLOB)) ELSE 0 END) WHERE family = 'sources'; END;

CREATE TRIGGER github_budget_source_snapshot_files_delete AFTER DELETE ON source_snapshot_files
BEGIN UPDATE github_storage_ledger SET bytes = bytes + (-(CASE WHEN OLD.collection IN (SELECT collection FROM github_code_scopes UNION SELECT collection FROM github_graph_scopes) THEN length(CAST(OLD.path AS BLOB))+length(CAST(OLD.source AS BLOB))+length(CAST(OLD.blob AS BLOB))+length(CAST(OLD.file_url AS BLOB)) ELSE 0 END)) WHERE family = 'sources'; END;

CREATE TRIGGER github_budget_source_snapshot_files_update AFTER UPDATE ON source_snapshot_files
BEGIN UPDATE github_storage_ledger SET bytes = bytes + ((CASE WHEN NEW.collection IN (SELECT collection FROM github_code_scopes UNION SELECT collection FROM github_graph_scopes) THEN length(CAST(NEW.path AS BLOB))+length(CAST(NEW.source AS BLOB))+length(CAST(NEW.blob AS BLOB))+length(CAST(NEW.file_url AS BLOB)) ELSE 0 END)-(CASE WHEN OLD.collection IN (SELECT collection FROM github_code_scopes UNION SELECT collection FROM github_graph_scopes) THEN length(CAST(OLD.path AS BLOB))+length(CAST(OLD.source AS BLOB))+length(CAST(OLD.blob AS BLOB))+length(CAST(OLD.file_url AS BLOB)) ELSE 0 END)) WHERE family = 'sources'; END;

CREATE TRIGGER github_budget_source_snapshots_insert AFTER INSERT ON source_snapshots
BEGIN UPDATE github_storage_ledger SET bytes = bytes + (CASE WHEN NEW.collection IN (SELECT collection FROM github_code_scopes UNION SELECT collection FROM github_graph_scopes) THEN length(CAST(NEW.revision AS BLOB))+length(CAST(NEW.tracked_ref AS BLOB))+length(CAST(coalesce(NEW.cursor,'') AS BLOB)) ELSE 0 END) WHERE family = 'sources'; END;

CREATE TRIGGER github_budget_source_snapshots_delete AFTER DELETE ON source_snapshots
BEGIN UPDATE github_storage_ledger SET bytes = bytes + (-(CASE WHEN OLD.collection IN (SELECT collection FROM github_code_scopes UNION SELECT collection FROM github_graph_scopes) THEN length(CAST(OLD.revision AS BLOB))+length(CAST(OLD.tracked_ref AS BLOB))+length(CAST(coalesce(OLD.cursor,'') AS BLOB)) ELSE 0 END)) WHERE family = 'sources'; END;

CREATE TRIGGER github_budget_source_snapshots_update AFTER UPDATE ON source_snapshots
BEGIN UPDATE github_storage_ledger SET bytes = bytes + ((CASE WHEN NEW.collection IN (SELECT collection FROM github_code_scopes UNION SELECT collection FROM github_graph_scopes) THEN length(CAST(NEW.revision AS BLOB))+length(CAST(NEW.tracked_ref AS BLOB))+length(CAST(coalesce(NEW.cursor,'') AS BLOB)) ELSE 0 END)-(CASE WHEN OLD.collection IN (SELECT collection FROM github_code_scopes UNION SELECT collection FROM github_graph_scopes) THEN length(CAST(OLD.revision AS BLOB))+length(CAST(OLD.tracked_ref AS BLOB))+length(CAST(coalesce(OLD.cursor,'') AS BLOB)) ELSE 0 END)) WHERE family = 'sources'; END;

CREATE TRIGGER github_budget_documents_insert AFTER INSERT ON documents
BEGIN UPDATE github_storage_ledger SET bytes = bytes + (CASE WHEN NEW.collection IN (SELECT collection FROM github_code_scopes UNION SELECT collection FROM github_graph_scopes) THEN length(CAST(NEW.title AS BLOB))+length(CAST(NEW.content AS BLOB))+length(CAST(coalesce(NEW.source_location,'') AS BLOB)) ELSE 0 END) WHERE family = 'sources'; END;

CREATE TRIGGER github_budget_documents_delete AFTER DELETE ON documents
BEGIN UPDATE github_storage_ledger SET bytes = bytes + (-(CASE WHEN OLD.collection IN (SELECT collection FROM github_code_scopes UNION SELECT collection FROM github_graph_scopes) THEN length(CAST(OLD.title AS BLOB))+length(CAST(OLD.content AS BLOB))+length(CAST(coalesce(OLD.source_location,'') AS BLOB)) ELSE 0 END)) WHERE family = 'sources'; END;

CREATE TRIGGER github_budget_documents_update AFTER UPDATE ON documents
BEGIN UPDATE github_storage_ledger SET bytes = bytes + ((CASE WHEN NEW.collection IN (SELECT collection FROM github_code_scopes UNION SELECT collection FROM github_graph_scopes) THEN length(CAST(NEW.title AS BLOB))+length(CAST(NEW.content AS BLOB))+length(CAST(coalesce(NEW.source_location,'') AS BLOB)) ELSE 0 END)-(CASE WHEN OLD.collection IN (SELECT collection FROM github_code_scopes UNION SELECT collection FROM github_graph_scopes) THEN length(CAST(OLD.title AS BLOB))+length(CAST(OLD.content AS BLOB))+length(CAST(coalesce(OLD.source_location,'') AS BLOB)) ELSE 0 END)) WHERE family = 'sources'; END;

CREATE TRIGGER github_budget_document_vectors_insert AFTER INSERT ON document_vectors
BEGIN UPDATE github_storage_ledger SET bytes = bytes + (CASE WHEN EXISTS (SELECT 1 FROM documents d WHERE d.id=NEW.document_id AND d.collection IN (SELECT collection FROM github_code_scopes UNION SELECT collection FROM github_graph_scopes)) THEN length(NEW.embedding)+length(CAST(NEW.profile AS BLOB)) ELSE 0 END) WHERE family = 'sources'; END;

CREATE TRIGGER github_budget_document_vectors_delete AFTER DELETE ON document_vectors
BEGIN UPDATE github_storage_ledger SET bytes = bytes + (-(CASE WHEN EXISTS (SELECT 1 FROM documents d WHERE d.id=OLD.document_id AND d.collection IN (SELECT collection FROM github_code_scopes UNION SELECT collection FROM github_graph_scopes)) THEN length(OLD.embedding)+length(CAST(OLD.profile AS BLOB)) ELSE 0 END)) WHERE family = 'sources'; END;

CREATE TRIGGER github_budget_document_vectors_update AFTER UPDATE ON document_vectors
BEGIN UPDATE github_storage_ledger SET bytes = bytes + ((CASE WHEN EXISTS (SELECT 1 FROM documents d WHERE d.id=NEW.document_id AND d.collection IN (SELECT collection FROM github_code_scopes UNION SELECT collection FROM github_graph_scopes)) THEN length(NEW.embedding)+length(CAST(NEW.profile AS BLOB)) ELSE 0 END)-(CASE WHEN EXISTS (SELECT 1 FROM documents d WHERE d.id=OLD.document_id AND d.collection IN (SELECT collection FROM github_code_scopes UNION SELECT collection FROM github_graph_scopes)) THEN length(OLD.embedding)+length(CAST(OLD.profile AS BLOB)) ELSE 0 END)) WHERE family = 'sources'; END;

CREATE TRIGGER github_budget_github_history_lineage_insert AFTER INSERT ON github_history_lineage
BEGIN UPDATE github_storage_ledger SET bytes = bytes + (length(CAST(NEW.snapshot AS BLOB))) WHERE family = 'history'; END;

CREATE TRIGGER github_budget_github_history_lineage_delete AFTER DELETE ON github_history_lineage
BEGIN UPDATE github_storage_ledger SET bytes = bytes + (-(length(CAST(OLD.snapshot AS BLOB)))) WHERE family = 'history'; END;

CREATE TRIGGER github_budget_github_history_lineage_update AFTER UPDATE ON github_history_lineage
BEGIN UPDATE github_storage_ledger SET bytes = bytes + ((length(CAST(NEW.snapshot AS BLOB)))-(length(CAST(OLD.snapshot AS BLOB)))) WHERE family = 'history'; END;

CREATE TRIGGER github_budget_github_history_evidence_insert AFTER INSERT ON github_history_evidence
BEGIN UPDATE github_storage_ledger SET bytes = bytes + (length(CAST(NEW.document AS BLOB))) WHERE family = 'history'; END;

CREATE TRIGGER github_budget_github_history_evidence_delete AFTER DELETE ON github_history_evidence
BEGIN UPDATE github_storage_ledger SET bytes = bytes + (-(length(CAST(OLD.document AS BLOB)))) WHERE family = 'history'; END;

CREATE TRIGGER github_budget_github_history_evidence_update AFTER UPDATE ON github_history_evidence
BEGIN UPDATE github_storage_ledger SET bytes = bytes + ((length(CAST(NEW.document AS BLOB)))-(length(CAST(OLD.document AS BLOB)))) WHERE family = 'history'; END;

CREATE TRIGGER github_budget_github_history_blobs_insert AFTER INSERT ON github_history_blobs
BEGIN UPDATE github_storage_ledger SET bytes = bytes + (length(NEW.bytes)) WHERE family = 'history'; END;

CREATE TRIGGER github_budget_github_history_blobs_delete AFTER DELETE ON github_history_blobs
BEGIN UPDATE github_storage_ledger SET bytes = bytes + (-(length(OLD.bytes))) WHERE family = 'history'; END;

CREATE TRIGGER github_budget_github_history_blobs_update AFTER UPDATE ON github_history_blobs
BEGIN UPDATE github_storage_ledger SET bytes = bytes + ((length(NEW.bytes))-(length(OLD.bytes))) WHERE family = 'history'; END;

CREATE TRIGGER github_budget_github_actions_snapshots_insert AFTER INSERT ON github_actions_snapshots
BEGIN UPDATE github_storage_ledger SET bytes = bytes + (length(CAST(NEW.snapshot AS BLOB))) WHERE family = 'actions'; END;

CREATE TRIGGER github_budget_github_actions_snapshots_delete AFTER DELETE ON github_actions_snapshots
BEGIN UPDATE github_storage_ledger SET bytes = bytes + (-(length(CAST(OLD.snapshot AS BLOB)))) WHERE family = 'actions'; END;

CREATE TRIGGER github_budget_github_actions_snapshots_update AFTER UPDATE ON github_actions_snapshots
BEGIN UPDATE github_storage_ledger SET bytes = bytes + ((length(CAST(NEW.snapshot AS BLOB)))-(length(CAST(OLD.snapshot AS BLOB)))) WHERE family = 'actions'; END;

CREATE TRIGGER github_budget_github_graph_publications_insert AFTER INSERT ON github_graph_publications
BEGIN UPDATE github_storage_ledger SET bytes = bytes + (length(CAST(NEW.roots AS BLOB))+length(CAST(NEW.coverage AS BLOB))+length(CAST(NEW.profile AS BLOB))+length(CAST(NEW.revision AS BLOB))) WHERE family = 'graph'; END;

CREATE TRIGGER github_budget_github_graph_publications_delete AFTER DELETE ON github_graph_publications
BEGIN UPDATE github_storage_ledger SET bytes = bytes + (-(length(CAST(OLD.roots AS BLOB))+length(CAST(OLD.coverage AS BLOB))+length(CAST(OLD.profile AS BLOB))+length(CAST(OLD.revision AS BLOB)))) WHERE family = 'graph'; END;

CREATE TRIGGER github_budget_github_graph_publications_update AFTER UPDATE ON github_graph_publications
BEGIN UPDATE github_storage_ledger SET bytes = bytes + ((length(CAST(NEW.roots AS BLOB))+length(CAST(NEW.coverage AS BLOB))+length(CAST(NEW.profile AS BLOB))+length(CAST(NEW.revision AS BLOB)))-(length(CAST(OLD.roots AS BLOB))+length(CAST(OLD.coverage AS BLOB))+length(CAST(OLD.profile AS BLOB))+length(CAST(OLD.revision AS BLOB)))) WHERE family = 'graph'; END;

CREATE TRIGGER github_budget_github_graph_occurrences_insert AFTER INSERT ON github_graph_occurrences
BEGIN UPDATE github_storage_ledger SET bytes = bytes + (length(CAST(NEW.record AS BLOB))+length(CAST(NEW.root AS BLOB))+length(CAST(NEW.name AS BLOB))+length(CAST(coalesce(NEW.qualified_name,'') AS BLOB))) WHERE family = 'graph'; END;

CREATE TRIGGER github_budget_github_graph_occurrences_delete AFTER DELETE ON github_graph_occurrences
BEGIN UPDATE github_storage_ledger SET bytes = bytes + (-(length(CAST(OLD.record AS BLOB))+length(CAST(OLD.root AS BLOB))+length(CAST(OLD.name AS BLOB))+length(CAST(coalesce(OLD.qualified_name,'') AS BLOB)))) WHERE family = 'graph'; END;

CREATE TRIGGER github_budget_github_graph_occurrences_update AFTER UPDATE ON github_graph_occurrences
BEGIN UPDATE github_storage_ledger SET bytes = bytes + ((length(CAST(NEW.record AS BLOB))+length(CAST(NEW.root AS BLOB))+length(CAST(NEW.name AS BLOB))+length(CAST(coalesce(NEW.qualified_name,'') AS BLOB)))-(length(CAST(OLD.record AS BLOB))+length(CAST(OLD.root AS BLOB))+length(CAST(OLD.name AS BLOB))+length(CAST(coalesce(OLD.qualified_name,'') AS BLOB)))) WHERE family = 'graph'; END;

-- A retained collection is a stable accounting owner. It can be removed only
-- once its payload has been purged; passages never move between owners.
CREATE TRIGGER github_budget_document_owner_immutable BEFORE UPDATE OF collection ON documents
WHEN OLD.collection <> NEW.collection AND (
 OLD.collection IN (SELECT collection FROM github_code_scopes UNION SELECT collection FROM github_graph_scopes)
 OR NEW.collection IN (SELECT collection FROM github_code_scopes UNION SELECT collection FROM github_graph_scopes))
BEGIN SELECT RAISE(ABORT, 'github_owner_immutable'); END;
CREATE TRIGGER github_budget_code_scope_owner_immutable BEFORE UPDATE OF collection ON github_code_scopes
WHEN OLD.collection <> NEW.collection
BEGIN SELECT RAISE(ABORT, 'github_owner_immutable'); END;
CREATE TRIGGER github_budget_graph_scope_owner_immutable BEFORE UPDATE OF collection ON github_graph_scopes
WHEN OLD.collection <> NEW.collection
BEGIN SELECT RAISE(ABORT, 'github_owner_immutable'); END;
CREATE TRIGGER github_budget_code_scope_retained BEFORE DELETE ON github_code_scopes
WHEN EXISTS (SELECT 1 FROM documents WHERE collection=OLD.collection)
  OR EXISTS (SELECT 1 FROM source_snapshots WHERE collection=OLD.collection)
  OR EXISTS (SELECT 1 FROM source_blobs WHERE collection=OLD.collection)
BEGIN SELECT RAISE(ABORT, 'github_owner_in_use'); END;
CREATE TRIGGER github_budget_graph_scope_retained BEFORE DELETE ON github_graph_scopes
WHEN EXISTS (SELECT 1 FROM documents WHERE collection=OLD.collection)
  OR EXISTS (SELECT 1 FROM source_snapshots WHERE collection=OLD.collection)
  OR EXISTS (SELECT 1 FROM source_blobs WHERE collection=OLD.collection)
BEGIN SELECT RAISE(ABORT, 'github_owner_in_use'); END;
CREATE TRIGGER github_budget_code_scope_no_reattribution BEFORE INSERT ON github_code_scopes
WHEN NOT EXISTS (SELECT 1 FROM github_code_scopes WHERE collection=NEW.collection)
 AND NOT EXISTS (SELECT 1 FROM github_graph_scopes WHERE collection=NEW.collection)
 AND (EXISTS (SELECT 1 FROM documents WHERE collection=NEW.collection)
   OR EXISTS (SELECT 1 FROM source_snapshots WHERE collection=NEW.collection)
   OR EXISTS (SELECT 1 FROM source_blobs WHERE collection=NEW.collection))
BEGIN SELECT RAISE(ABORT, 'github_owner_in_use'); END;
CREATE TRIGGER github_budget_graph_scope_no_reattribution BEFORE INSERT ON github_graph_scopes
WHEN NOT EXISTS (SELECT 1 FROM github_code_scopes WHERE collection=NEW.collection)
 AND NOT EXISTS (SELECT 1 FROM github_graph_scopes WHERE collection=NEW.collection)
 AND (EXISTS (SELECT 1 FROM documents WHERE collection=NEW.collection)
   OR EXISTS (SELECT 1 FROM source_snapshots WHERE collection=NEW.collection)
   OR EXISTS (SELECT 1 FROM source_blobs WHERE collection=NEW.collection))
BEGIN SELECT RAISE(ABORT, 'github_owner_in_use'); END;
