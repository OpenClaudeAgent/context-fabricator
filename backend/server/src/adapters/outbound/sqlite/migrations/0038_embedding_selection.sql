CREATE TABLE embedding_selection (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    profile TEXT NOT NULL CHECK (profile <> '')
) STRICT;
