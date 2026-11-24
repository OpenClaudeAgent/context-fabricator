ALTER TABLE github_history_generations ADD COLUMN latest_trace BLOB
    CHECK(latest_trace IS NULL OR length(latest_trace) = 24);
ALTER TABLE github_actions_generations ADD COLUMN latest_trace BLOB
    CHECK(latest_trace IS NULL OR length(latest_trace) = 24);

ALTER TABLE github_history_lineage ADD COLUMN published_trace BLOB
    CHECK(published_trace IS NULL OR length(published_trace) = 24);
ALTER TABLE github_actions_snapshots ADD COLUMN published_trace BLOB
    CHECK(published_trace IS NULL OR length(published_trace) = 24);

CREATE TRIGGER github_history_trace_generation_changed
AFTER UPDATE OF generation ON github_history_generations
WHEN NEW.generation <> OLD.generation
BEGIN
    UPDATE github_history_generations SET latest_trace = NULL WHERE repository_id = OLD.repository_id;
END;

CREATE TRIGGER github_actions_trace_generation_changed
AFTER UPDATE OF generation ON github_actions_generations
WHEN NEW.generation <> OLD.generation
BEGIN
    UPDATE github_actions_generations SET latest_trace = NULL WHERE repository_id = OLD.repository_id;
END;
