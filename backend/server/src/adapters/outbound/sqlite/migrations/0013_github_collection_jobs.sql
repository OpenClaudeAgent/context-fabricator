ALTER TABLE github_code_scopes ADD COLUMN last_checked_at_ms INTEGER;
ALTER TABLE github_code_scopes ADD COLUMN last_error TEXT;
ALTER TABLE source_snapshots ADD COLUMN published_at_ms INTEGER;

CREATE VIEW github_current_eligible AS
SELECT r.id AS repository_id, r.full_name, r.branch, r.interval_minutes,
       s.collection, c.generation, a.credential_ref
FROM github_repositories r
JOIN github_code_scopes s ON (s.repository_id, s.branch) = (r.id, r.branch)
JOIN collections c ON c.name = s.collection
CROSS JOIN github_connection a
WHERE r.verified AND r.current_code AND r.contents_access = 1
  AND NOT c.access_lost AND NOT a.reconnect AND a.credential_ref IS NOT NULL;
