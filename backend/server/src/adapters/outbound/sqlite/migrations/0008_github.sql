CREATE TABLE github_connection (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    revision INTEGER NOT NULL DEFAULT 0 CHECK (revision BETWEEN 0 AND 4294967295),
    credential_ref TEXT,
    login TEXT,
    reconnect INTEGER NOT NULL DEFAULT 0 CHECK (reconnect IN (0, 1)),
    CHECK ((credential_ref IS NULL) = (login IS NULL))
) STRICT;
INSERT INTO github_connection (singleton) VALUES (1);

CREATE TABLE github_repositories (
    id INTEGER PRIMARY KEY CHECK (id > 0),
    full_name TEXT NOT NULL,
    default_branch TEXT NOT NULL,
    private INTEGER NOT NULL CHECK (private IN (0, 1)),
    branch TEXT NOT NULL,
    interval_minutes INTEGER NOT NULL CHECK (interval_minutes BETWEEN 15 AND 1440),
    current_code INTEGER NOT NULL CHECK (current_code IN (0, 1)),
    history INTEGER NOT NULL CHECK (history IN (0, 1)),
    actions INTEGER NOT NULL CHECK (actions IN (0, 1)),
    code_graph INTEGER NOT NULL CHECK (code_graph IN (0, 1)),
    contents_access INTEGER CHECK (contents_access IN (0, 1)),
    actions_access INTEGER CHECK (actions_access IN (0, 1)),
    verified INTEGER NOT NULL CHECK (verified IN (0, 1))
) STRICT;
