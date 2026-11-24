-- Current-code and graph-only manifests have independent retained owners.
CREATE TABLE github_graph_scopes (
    collection TEXT PRIMARY KEY REFERENCES collections(name) DEFERRABLE INITIALLY DEFERRED,
    repository_id INTEGER NOT NULL, branch TEXT NOT NULL,
    last_checked_at_ms INTEGER, last_error TEXT,
    UNIQUE(repository_id, branch)
) STRICT;

INSERT INTO github_graph_scopes(collection,repository_id,branch)
SELECT 'github/graph/' || r.id || '/' || r.branch || '#' || lower(hex(randomblob(16))), r.id, r.branch
FROM github_repositories r CROSS JOIN github_connection c
WHERE r.code_graph AND NOT r.current_code AND r.verified AND r.contents_access = 1
  AND c.credential_ref IS NOT NULL AND NOT c.reconnect;
INSERT INTO collections(name) SELECT collection FROM github_graph_scopes;

-- Source scheduling chooses exactly one manifest per active repository and branch.
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
  AND r.contents_access = 1 AND NOT c.access_lost AND NOT a.reconnect AND a.credential_ref IS NOT NULL;

-- A change of projections invalidates already staged work, even when the ref has not moved.
CREATE TRIGGER github_graph_projection_changed AFTER UPDATE OF code_graph ON github_repositories
WHEN OLD.code_graph <> NEW.code_graph AND NEW.current_code
BEGIN
    UPDATE collections SET generation = generation + 1
      WHERE name IN (SELECT collection FROM github_code_scopes WHERE repository_id = NEW.id AND branch = NEW.branch);
END;
UPDATE collections SET generation = generation + 1 WHERE name IN (
    SELECT s.collection FROM github_code_scopes s JOIN github_repositories r
      ON (s.repository_id,s.branch)=(r.id,r.branch) WHERE r.code_graph AND r.current_code
);

CREATE TABLE github_graph_publications (
    repository_id INTEGER NOT NULL,
    branch TEXT NOT NULL,
    revision TEXT NOT NULL,
    profile TEXT NOT NULL,
    coverage TEXT NOT NULL,
    roots TEXT NOT NULL CHECK(json_valid(roots)),
    PRIMARY KEY(repository_id, branch)
) STRICT;
CREATE TABLE github_graph_occurrences (
    repository_id INTEGER NOT NULL, branch TEXT NOT NULL, root TEXT NOT NULL,
    ordinal INTEGER NOT NULL, name TEXT NOT NULL, qualified_name TEXT, record TEXT NOT NULL CHECK(json_valid(record)),
    PRIMARY KEY(repository_id,branch,root,ordinal),
    FOREIGN KEY(repository_id,branch) REFERENCES github_graph_publications(repository_id,branch) ON DELETE CASCADE
) STRICT;
CREATE INDEX github_graph_symbol ON github_graph_occurrences(repository_id,branch,root,name,ordinal);
CREATE INDEX github_graph_qualified_symbol ON github_graph_occurrences(repository_id,branch,root,qualified_name,ordinal);
