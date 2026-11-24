CREATE TABLE github_history_lineage (
    repository_id INTEGER NOT NULL CHECK (repository_id > 0),
    branch TEXT NOT NULL,
    snapshot TEXT NOT NULL CHECK (json_valid(snapshot)),
    PRIMARY KEY (repository_id, branch)
) STRICT;

CREATE TABLE github_history_generations (
    repository_id INTEGER PRIMARY KEY CHECK (repository_id > 0),
    generation INTEGER NOT NULL CHECK (generation >= 0)
) STRICT;

INSERT INTO github_history_generations (repository_id, generation)
SELECT id, 0 FROM github_repositories;

CREATE TRIGGER github_history_configured AFTER INSERT ON github_repositories
BEGIN
    INSERT INTO github_history_generations (repository_id, generation) VALUES (NEW.id, 0)
    ON CONFLICT (repository_id) DO UPDATE SET generation = generation + 1;
END;

CREATE TRIGGER github_history_changed AFTER UPDATE ON github_repositories
WHEN OLD.branch IS NOT NEW.branch OR OLD.full_name IS NOT NEW.full_name
     OR OLD.history IS NOT NEW.history OR OLD.contents_access IS NOT NEW.contents_access
     OR OLD.verified IS NOT NEW.verified
BEGIN
    UPDATE github_history_generations SET generation = generation + 1 WHERE repository_id = NEW.id;
END;

CREATE TRIGGER github_history_removed AFTER DELETE ON github_repositories
BEGIN
    UPDATE github_history_generations SET generation = generation + 1 WHERE repository_id = OLD.id;
END;
