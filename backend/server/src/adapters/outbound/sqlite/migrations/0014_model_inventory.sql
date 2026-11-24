CREATE TABLE model_inventory (
    profile TEXT PRIMARY KEY,
    artifact TEXT NOT NULL,
    verified INTEGER NOT NULL DEFAULT 0 CHECK (verified IN (0, 1))
) STRICT;
