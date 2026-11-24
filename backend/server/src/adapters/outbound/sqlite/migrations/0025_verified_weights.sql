CREATE TABLE verified_weights (
    profile TEXT PRIMARY KEY,
    file_name TEXT NOT NULL,
    sha256 TEXT NOT NULL,
    identity TEXT NOT NULL
) STRICT;
