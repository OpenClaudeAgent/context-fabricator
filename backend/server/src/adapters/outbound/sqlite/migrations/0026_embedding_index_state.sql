CREATE TABLE embedding_index_state (
    profile TEXT PRIMARY KEY,
    enabled INTEGER NOT NULL CHECK (enabled IN (0, 1)),
    epoch INTEGER NOT NULL CHECK (epoch >= 0)
) STRICT;
