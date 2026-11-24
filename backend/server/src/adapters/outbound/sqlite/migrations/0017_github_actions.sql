CREATE TABLE github_actions_snapshots (
    repository_id INTEGER NOT NULL CHECK (repository_id > 0),
    branch TEXT NOT NULL,
    snapshot TEXT NOT NULL CHECK (json_valid(snapshot)),
    PRIMARY KEY (repository_id, branch)
) STRICT;

CREATE TABLE github_actions_generations (
    repository_id INTEGER PRIMARY KEY CHECK (repository_id > 0),
    generation INTEGER NOT NULL CHECK (generation >= 0)
) STRICT;
INSERT INTO github_actions_generations (repository_id, generation) SELECT id, 0 FROM github_repositories;
CREATE TRIGGER github_actions_configured AFTER INSERT ON github_repositories BEGIN
    INSERT INTO github_actions_generations (repository_id, generation) VALUES (NEW.id, 0)
    ON CONFLICT (repository_id) DO UPDATE SET generation = generation + 1;
END;
CREATE TRIGGER github_actions_changed AFTER UPDATE ON github_repositories
WHEN OLD.branch IS NOT NEW.branch OR OLD.full_name IS NOT NEW.full_name OR OLD.actions IS NOT NEW.actions
  OR OLD.actions_access IS NOT NEW.actions_access OR OLD.contents_access IS NOT NEW.contents_access OR OLD.verified IS NOT NEW.verified
  OR OLD.interval_minutes IS NOT NEW.interval_minutes
BEGIN
    UPDATE github_actions_generations SET generation = generation + 1 WHERE repository_id = NEW.id;
END;
CREATE TRIGGER github_actions_removed AFTER DELETE ON github_repositories BEGIN
    UPDATE github_actions_generations SET generation = generation + 1 WHERE repository_id = OLD.id;
END;

CREATE TABLE github_actions_schedule (
    repository_id INTEGER PRIMARY KEY CHECK (repository_id > 0),
    generation INTEGER NOT NULL CHECK (generation >= 0),
    next_at_ms INTEGER NOT NULL CHECK (next_at_ms >= 0),
    last_checked_at_ms INTEGER NOT NULL CHECK (last_checked_at_ms >= 0),
    last_error TEXT
) STRICT;

CREATE TRIGGER github_actions_credential_changed AFTER UPDATE ON github_connection
WHEN OLD.credential_ref IS NOT NEW.credential_ref OR OLD.reconnect IS NOT NEW.reconnect
BEGIN
    UPDATE github_actions_generations SET generation = generation + 1;
END;
