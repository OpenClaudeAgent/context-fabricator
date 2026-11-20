CREATE TABLE github_code_scopes (
    collection TEXT PRIMARY KEY NOT NULL REFERENCES collections(name) DEFERRABLE INITIALLY DEFERRED,
    repository_id INTEGER NOT NULL,
    branch TEXT NOT NULL,
    UNIQUE(repository_id, branch)
) STRICT;

INSERT INTO github_code_scopes
SELECT 'github/current/' || r.id || '/' || r.branch ||
       CASE WHEN EXISTS (SELECT 1 FROM collections
                        WHERE name = 'github/current/' || r.id || '/' || r.branch)
            THEN '#' || lower(hex(randomblob(16))) ELSE '' END, r.id, r.branch
FROM github_repositories r CROSS JOIN github_connection c
WHERE r.verified AND r.current_code AND r.contents_access = 1
  AND c.credential_ref IS NOT NULL AND NOT c.reconnect;

INSERT INTO collections (name) SELECT collection FROM github_code_scopes;
