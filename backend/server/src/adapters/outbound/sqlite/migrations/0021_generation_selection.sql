CREATE TABLE generation_selection (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    profile TEXT NOT NULL CHECK (length(profile) BETWEEN 1 AND 128),
    fingerprint TEXT NOT NULL CHECK (length(fingerprint) = 64)
) STRICT;
