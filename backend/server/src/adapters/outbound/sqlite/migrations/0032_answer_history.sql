CREATE TABLE answer_history_settings (
    singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
    revision INTEGER NOT NULL DEFAULT 0,
    enabled INTEGER NOT NULL DEFAULT 0 CHECK(enabled IN (0, 1)),
    retention_days INTEGER NOT NULL DEFAULT 30 CHECK(retention_days BETWEEN 1 AND 365),
    epoch INTEGER NOT NULL DEFAULT 0,
    purge_sequence INTEGER NOT NULL DEFAULT 0
);
INSERT INTO answer_history_settings(singleton) VALUES (1);
CREATE TABLE answer_history_purges (
    repository_id INTEGER PRIMARY KEY,
    sequence INTEGER NOT NULL
);
CREATE TABLE answer_history_entries (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    question TEXT NOT NULL,
    scope_json TEXT NOT NULL,
    completed_at_ms INTEGER NOT NULL,
    answer_version INTEGER NOT NULL,
    answer_json TEXT NOT NULL,
    outcome TEXT NOT NULL
);
CREATE INDEX answer_history_completed ON answer_history_entries(completed_at_ms);
CREATE TABLE answer_history_owners (
    entry_id INTEGER NOT NULL REFERENCES answer_history_entries(id) ON DELETE CASCADE,
    repository_id INTEGER NOT NULL,
    PRIMARY KEY(entry_id, repository_id)
);
CREATE INDEX answer_history_owners_repository ON answer_history_owners(repository_id);
