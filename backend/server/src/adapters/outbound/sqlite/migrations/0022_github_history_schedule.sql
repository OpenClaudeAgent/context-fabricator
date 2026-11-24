CREATE TABLE github_history_schedule (
    repository_id INTEGER PRIMARY KEY,
    generation INTEGER NOT NULL,
    last_checked_at_ms INTEGER NOT NULL CHECK(last_checked_at_ms >= 0),
    last_error TEXT
) STRICT;

CREATE VIEW github_history_eligible AS
SELECT r.id AS repository_id, r.full_name, r.branch, g.generation, c.credential_ref,
       r.interval_minutes, s.last_checked_at_ms, s.last_error,
       CASE WHEN s.repository_id IS NULL THEN 0
            WHEN s.last_error IS NOT NULL THEN s.last_checked_at_ms + 60000
            ELSE s.last_checked_at_ms + r.interval_minutes * 60000 END AS next_at_ms
FROM github_repositories r
JOIN github_history_generations g ON g.repository_id = r.id
CROSS JOIN github_connection c
LEFT JOIN github_history_schedule s ON (s.repository_id,s.generation) = (r.id,g.generation)
WHERE r.history = 1 AND r.verified = 1 AND r.contents_access = 1
  AND c.singleton=1 AND c.reconnect=0 AND c.credential_ref IS NOT NULL
  AND NOT EXISTS (SELECT 1 FROM github_data_paused p WHERE p.repository_id=r.id);
