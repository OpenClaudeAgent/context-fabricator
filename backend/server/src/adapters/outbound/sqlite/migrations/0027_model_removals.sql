CREATE TABLE model_removals (
    profile TEXT PRIMARY KEY,
    fingerprint TEXT NOT NULL,
    identity TEXT NOT NULL
) STRICT;
