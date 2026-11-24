CREATE TABLE github_history_evidence (
    repository_id INTEGER NOT NULL CHECK (repository_id > 0),
    branch TEXT NOT NULL,
    child TEXT NOT NULL,
    parent TEXT NOT NULL,
    projection_version INTEGER NOT NULL CHECK (projection_version > 0),
    document TEXT NOT NULL CHECK (json_valid(document)),
    PRIMARY KEY (repository_id, branch, child, parent, projection_version)
) STRICT;

CREATE TABLE github_history_blobs (
    blob TEXT PRIMARY KEY,
    bytes BLOB NOT NULL
) STRICT;

CREATE TABLE github_history_evidence_blobs (
    repository_id INTEGER NOT NULL,
    branch TEXT NOT NULL,
    child TEXT NOT NULL,
    parent TEXT NOT NULL,
    projection_version INTEGER NOT NULL,
    blob TEXT NOT NULL REFERENCES github_history_blobs(blob),
    PRIMARY KEY (repository_id, branch, child, parent, projection_version, blob),
    FOREIGN KEY (repository_id, branch, child, parent, projection_version)
        REFERENCES github_history_evidence (repository_id, branch, child, parent, projection_version)
        ON DELETE CASCADE
) STRICT;

CREATE TRIGGER github_history_orphan_blob AFTER DELETE ON github_history_evidence_blobs
BEGIN
    DELETE FROM github_history_blobs WHERE blob = OLD.blob
      AND NOT EXISTS (SELECT 1 FROM github_history_evidence_blobs WHERE blob = OLD.blob);
END;
